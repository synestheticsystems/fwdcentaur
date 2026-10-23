# Manifold and fwdcentaur: crossover notes

Written 2026-09-24 from a read of the private `ankurah/manifold` packet (README, DECISIONS, docs 00, 01, 02, 04, 06, 07, 10, 13, 18, 19, the addressing/annotation prior-art inventory, the Ankurah 0.9.3 modeling inventory, and the model-contract probe README). The rendering documents (05, 12, 14–17, 09, verification) are summarized separately in `manifold-rendering.md`. This note follows Manifold's own convention: it separates what Daniel established there from what its packet proposes.

## What Manifold is, in one paragraph

A local place to think with connected knowledge: notes, concepts, questions, claims, sources, decisions and relationships, with tasks as an optional capability. Built on Ankurah with a separate bundled daemon shared by a GUI and a CLI. The packet's centre of gravity is the graph UI (projection without global coordinates, semantic zoom, placeness, Back) and a CLI whose commands mean the same thing as the GUI's. It is a design packet with synthetic experiments and a small executable probe; no application exists yet.

## Manifold decisions fwdcentaur should adopt unchanged

These are Daniel's established positions in Manifold's DECISIONS.md, and each one answers a question fwdcentaur would otherwise have to settle on its own.

1. **A separate bundled daemon owns the durable Ankurah node; GUI and CLI share one startup contract.** Manifold's architecture doc (04, "One startup contract for UI and CLI") specifies the bootstrap lock, the lifetime lock, the Hello handshake with peer-UID and protocol checks, the lifecycle state machine and the idle-exit grace. fwdcentaur's daemon is the same shape with one workspace per repository. If the two products merge, it is the same daemon.

2. **Relations are entities.** Manifold's rule (DECISIONS, "Edges and HyperEdges are entity conventions") is that an edge is an entity carrying two Refs, with named refs for endpoint roles, never a separate edge store. For fwdcentaur this is exactly right: every dependency, subtask link, motivation link and code anchor becomes an entity that can carry provenance (which annotation or which agent asserted it, when), can be commented on, and can be the target of a proposal. "Propose removing this dependency" is then a proposal targeting one relation entity.

3. **Interior pointing entity-ifies the selected interior.** Manifold's pointing contract (13) says that pointing at a point, span or region inside an otherwise indivisible entity establishes that selected thing as an entity with its own identity, defined by selectors plus a source basis, and that standoff annotations target that sub-entity rather than the enclosing source. fwdcentaur's line annotations are this contract applied to source code: the enclosing entity is a file at a blob, the sub-entity is a line range with selectors (path, blob hash, line range, content fingerprint, surrounding context), and the annotation targets the sub-entity. Manifold's three-way distinction for edits underneath a pointer, location certainty versus selected-content change versus availability, is the right vocabulary for re-anchoring across commits. The only difference is the version basis: git blobs and diffs instead of Yrs sticky indexes.

4. **Contextual pointing is a separate concept.** Pointing at a task as encountered in a particular scope or projection ("T72 as it appears in Alice's scope") qualifies the encounter and does not create a sub-entity. fwdcentaur's inbox messages will need both: a message can reference a sub-entity (the line range) and a context (the scope in which the annotator was working).

5. **The CLI grammar.** Manifold's CLI doc (06) specifies capability and schema discovery, stateless queries with explicit scope and bounds, a result envelope that separates input completeness from output truncation, checked writes with expected revisions and idempotency keys, a proposal lifecycle (propose, inspect, apply, reconcile), batch semantics without invented atomicity, a watch stream built from a snapshot plus keyed batches plus reset, and stable exit codes. fwdcentaur's CLI should be that grammar with task, scope, inbox and anchor namespaces, and possibly the same binary.

6. **The task model.** Manifold's domain doc (02, "Task state is optional") separates lifecycle (draft, open, active, review, done, cancelled) from commitment (idea, proposed, chosen, promised), derives readiness from three-valued prerequisite expressions, keeps availability overlays (hold, waiting, delegated, blocked) out of the lifecycle enum, treats an agent's self-report as a submission rather than an acceptance, and rejects prerequisite cycles without silently dropping edges. This is more careful than Beads' status enum and worth adopting from day one, because agents will drive most transitions and "done because the agent said so" is the failure mode to design against.

7. **Attribution and receipts.** Principal identity comes from authentication, not from an attribution field; agent runs are attributed observations; commands produce receipts that enumerate their effects. This is what makes a supervising agent's edits auditable.

8. **No fundamental coordinates.** Layout is projection output. The VS Code graph panel must not store node positions as entity properties; a visit-local layout cache is the most it may keep.

9. **Agnostic entities with behavioral components, Task being one.** Manifold wants this natively in Ankurah, sequenced after the AK Metadata branch lands and expands to multi-model membership (DECISIONS, "Land AK Metadata before expanding membership"). fwdcentaur does not need multi-component entities to ship: its nouns (task, annotation, anchor sub-entity, message, scope, principal, relation) are distinct referents, and an annotation that turns out to be a task is better modelled as a new task entity related by discovered-from than as a task component attached to the annotation. So fwdcentaur can start on ordinary single-collection Models, which the Manifold probe demonstrates work for relation entities and reverse live queries on 0.9.3, and migrate to components when Ankurah has them. Manifold's own advice applies: keep the compatibility boundary at application commands, typed target identities and projection results, so the substrate can change underneath.

## What fwdcentaur adds that Manifold does not have

- Git as the source basis for interior pointing, and re-anchoring of sub-entities across commits, rebases and refactors.
- VS Code as the host: annotations in the editor and diff view, and a graph panel in a webview.
- Inbox messages addressed to principals, with human annotations queued for an agent to interpret.
- Scopes: a per-person or per-agent working context with a status, an intention, assigned tasks, and local-only fields such as the working directory.
- Two agent roles (implementing, supervising) and the CLI conventions that let a plain Claude Code session play either one.
- Team sync from the start. Manifold defers remote access to a later iroh milestone; fwdcentaur needs several laptops sharing one graph early. Ankurah's WebSocket durable server and JWT RBAC extension exist today; the known gap is that the 0.9 connectors do not replay writes made while disconnected.
- An optional projection to GitHub Issues for coarse items, so teammates who live in GitHub still see milestones without the fine-grained noise.

## Apparent conflicts, and whether they are real

- **Webviews.** Manifold's requirement R31 excludes embedding external web content (iframes, browser-extension workarounds) and explicitly allows a WebView hosting Manifold's own UI. A VS Code webview hosting fwdcentaur's own graph panel is the allowed case, so there is no conflict. There is a dependency, though: if Manifold's renderer bakeoff picks Flutter, sharing one graph UI between Manifold and the VS Code panel becomes heavy (Flutter web inside a webview is possible but large and slow to start). If HTML/SVG wins, the panel can be the same code. fwdcentaur has no choice but HTML/SVG inside VS Code, which makes it a useful data point for the bakeoff rather than a competitor to it.
- **Timeline.** Manifold's substrate work (native components after AK Metadata) has no landing date and its renderer is unselected. fwdcentaur should not inherit those as blockers. Building fwdcentaur on ordinary Models now and sharing contracts, not code paths that do not exist yet, avoids that.
- **Single user versus team.** Manifold is single-owner local-first with remote modes later; fwdcentaur is a team tool. Both fit Ankurah; the difference is which connector work gets done first.
- **Naming.** Manifold uses `kg` and `kgd` as placeholders; fwdcentaur's working name is `fwd` (one binary; the daemon is `fwd daemon serve`).

## Is fwdcentaur part of Manifold?

Lean: fwdcentaur is a Manifold application, built first because its scope is narrower and its host is fixed. Concretely, share the entity, relation and interior-pointing semantics, the daemon lifecycle and CLI grammar, the Task component definition and, eventually, the Ankurah node and workspace. Do not share yet: the renderer decision, the placeness and semantic-zoom machinery, and the knowledge item types. Decide whether it is one product or two after fwdcentaur has a working vertical slice.

The risk of merging too early is that fwdcentaur inherits Manifold's open decisions as blockers. The risk of keeping them fully separate is two daemons, two CLIs and two incompatible task schemas. The mitigation is a shared Rust library (the application layer over Ankurah: daemon bootstrap, command receipts, proposal lifecycle, relation entities, interior pointing, watch streams) that both products depend on, with fwdcentaur as its first consumer.
