# Manifold rendering docs: what transfers to fwdcentaur's VS Code graph panel

Research note, 2026-09-24. A research agent read all eleven rendering-related documents in the private `ankurah/manifold` repository (outputs 03, 05, 09, 12, 14, 15, 16, 17, verification, and the layout/renderer prior-art inventories). All are dated 7 September 2026. The docs label Daniel's requirements, proposals ("P:"), and evidence; this note preserves those labels. Anything marked "inference" is the summarizer's, not the docs'.

## 1. Decided positions vs open alternatives

**Decided (recorded as Daniel's requirement, correction, or "accepted"):**

- Foundation, `outputs/12-rendering-methodology.md` opening: "Persist identity, relationships, authored intent and the route of thought. Compute geometry for the current projection." "An entity does not have a fundamental x/y/z location." This "supersedes the earlier packet's recommendation of a persistent authored spatial map."
- "Make Back the first way to regain orientation." Explicitly: "This is Daniel's requirement; the retained state and checkpoint mechanisms below are proposals."
- A separate bundled daemon owns data and checked operations for both GUI and CLI; "UI and CLI use the same semantics."
- "The core UI must run in the browser and native app through one rendering/interaction implementation."
- "Embedded external webviews are expressly out of scope now." (What this actually excludes is in section 4; it is narrower than it sounds.)
- Reuse an established component system for ordinary controls, with custom components for Bubble Sets, bands, edges. "This is an explicit reuse/extensibility preference; no framework has been selected."
- Conditional typography rule: "If components scale, typography ratchets in discrete steps inverse to that scale as semantic detail changes." Point-by-point font resizing is excluded. This "does not choose continuous component scaling."
- Presentation invariants exist (`outputs/15-presentation-invariants.md`): "Some aspects of an entity's or subgraph's presentation should survive every change of context; other aspects should adapt." The boundary and categories are proposals.
- Translucent bands are a primary encoding of deliberate association; "Subtle appearance describes attention demand, not uncertainty or weak commitment."
- Alignment principle: "Align semantic and visual axes where useful; make the kind and strength of alignment intelligible." The vocabulary for it is a proposal.
- Force-directed motion is not the default: "Continuous force motion is not the default reading/editing behavior."

**Explicitly open:**

- UI architecture: browser-owned HTML/SVG versus a complete toolkit (Flutter is "the leading non-HTML candidate in this bounded review"). "Final stack selection remains open."
- Component-size behavior: constant screen-size, large discrete steps, continuous scaling are "genuine alternatives", independent of the architecture choice.
- Tier thresholds and transition animations: "No winning cadence, minimum font size or threshold is established."
- Layout library: "No layout library is selected by this research." Dagre is a candidate (MIT); ELK "is not selected or cleared for reuse" because the license lookup failed.
- Back mechanics: checkpoint rules, retention, and whether to restore a retained presentation before incorporating live updates.
- Bubble Set marks and reveal interaction, gesture vocabulary, editor selection, presence encodings, browser transport.
- Local-model summarization for semantic zoom is "a mechanism to prototype"; "This pass runs no local model."

**Superseded formulations** (so nobody re-adopts them): the persistent spatial map; a mandatory separate Follow command; the 80-item/160-edge cap as "a perceptual law"; a separate SVG/resvg export renderer; the Mac-first native stack; the broad "DOM plus coordinated SVG/canvas/GPU" recommendation.

## 2. Projection and layout principles for hundreds-to-thousands of nodes

From `outputs/12-rendering-methodology.md` sections 2, 3 and 5 and `outputs/03-interaction-and-projection.md` section 2.

**A view carries an inspectable specification.** Question/scope, preserved distinctions, semantic/visual alignment, presentational commitments, transformations (filters, duplication, grouping, aggregation, path compression), boundaries ("omitted authorized relationships, truncation, unavailable inputs, and whether a total is actually known"), and interaction bindings. The presentation graph is distinct from the source graph: "an appearance has a provenance mapping to the source; a displayed aggregate has a membership mapping; a compressed path has witnesses."

**Selective rendering is several different omissions.** "Rendering is selective at several levels: query scope, relationships expressed, representation detail, viewport culling and resource loading. These are different omissions. An off-screen item is not filtered out of the query; an unloaded PDF is not absent knowledge; an aggregate is not a new entity." And: "Do not let line opacity be the only indication that relationships were omitted."

**Duplicated appearances.** "Duplicate an appearance, never the underlying entity merely to simplify a picture. Do not automatically reproduce all incident edges at every appearance. Each appearance states its contextual role; a consolidated relation list remains available. Stop recursive unfolding at scope boundaries and cycles, with an explicit reference that can be followed." Evidence cited: Henry, Bezerianos and Fekete 2008 found duplication helped community tasks but hurt some graph-reading tasks, and "merely coloring clones or highlighting them on selection was not sufficient." "Counts distinguish distinct subjects, appearances and relations."

**Aggregation and path compression.** "A compound result is not a newly created knowledge item. A compressed path names its witnesses rather than impersonating a direct relation. An outline is a chosen traversal, not a universal parent hierarchy." Aggregates "have no generic setter"; they can reveal authorized members, and "unknown totals stay unknown."

**Choose representation by inquiry before optimizing coordinates.** Directional dependencies map to layered layouts, comparison to ordered columns, a chosen traversal to trees, dense incidence to tables, "How do these things connect?" to "Selected path with expandable witnesses" where "Direct edge versus multi-step path; alternatives omitted" must stay explicit. Ghoniem, Fekete and Castagliola 2005 is cited for task dependence (matrices for dense reading, node-link for path finding) with the caveat that "its numerical size thresholds should not become our UI rules."

**Force versus layered.** "Force-directed layout is useful for exploratory topology and as a soft relaxation inside stronger constraints. Its costs are motion, local minima, sensitivity to the selected graph, uncertain distance meaning and poor accommodation of large readable content. Use it in a named analytical mode, or to suggest an arrangement that settles. It should not continuously rearrange the main reading/editing surface." "Crossing count alone is insufficient: a crossing-free detour can still be hard to follow." Temporary stability rules: "retain the hit target while a pointer is down, preserve the caret and selection, and show correspondence through a transition." "Rerunning layout immediately on Back must not erase the familiarity that return is meant to restore."

**Layout must not invent relationships or imply completeness.** "No projection is entitled to imply it preserves every dimension." "A region around A/B/C must not imply all three pairwise relations." "Absence of a band means no association displayed in this layer, not proof of no relationship anywhere in the graph." "Automatic layout, zoom, window resizing and remote reprojection never call semantic make/break logic." "A partial order such as dependency precedence must not be presented as a known total chronology. Cycles or inconsistent constraints require an explicit representation or unresolved state." "A prerequisite edge does not by itself establish an actual historical time or scheduled duration." Bundling "is particularly dangerous when a shared route appears to establish a junction."

**Alignment has a kind and a strength.** Five kinds (loose directional, ordinal/monotonic, calibrated nonlinear, linear affine, categorical) crossed with enforcement strength ("a soft preference, a mapping with bounded deviation, or a required invariant"). Pipeline stage 5: "Report unsatisfied commitments rather than silently weakening them."

**Semantic zoom.** "Semantic zoom allocates visual attention by changing representation and detail as scale, focus, context and available space change." Three variables (geometric magnification, detail, scope) "may cooperate through one fluid interaction; this is not a requirement for three separate visible controls." Candidate progression: "full content -> excerpt or paragraph summary -> sentence -> phrase -> single word, symbol or image", "not mandatory fixed stages." "Use hysteresis at representation boundaries and avoid relocating an active selection while content reflows." "Do not let body length, popularity or node degree automatically become apparent importance."

**Typography ratchet** (conditional on scaling): `outputs/17-scaling-and-dense-rendering.md` gives a scheduling model: camera translation/uniform scale should not trigger text reflow; width/typography/source change triggers text layout of affected content only; a tier step is evaluated "as a coherent generation"; "Instantiating every visible appearance as an active editor" is never indiscriminate. All marked "a proposed scheduling model to test."

**Bubble Sets for membership** (`outputs/16-bubble-sets-and-membership.md`): membership and association are separate visual jobs; "A contour neck is not an edge." Omission marks: `(+n)` for an exact count, `...` for known-but-uncounted, and a distinct unknown-completeness state ("do not substitute an exact zero"). Counts count "distinct member referents", not appearances. "An absent contour must not turn a nonempty hidden set into an apparently empty set." "Bubble area, neck width and tint strength do not automatically encode cardinality, relationship strength or certainty." Only the six-target synthetic specimen was browser-checked; the rest is "experiment questions."

**Visual pressure, not counts.** "Use measured visual pressure—text area, occlusion, fan-out near focus, label collisions, traceability and interaction latency—to choose fallbacks. Finite resource limits are still necessary, but the earlier 80-item/160-edge proposal is not a perceptual law."

## 3. Placeness and navigation

From `outputs/14-placeness-and-navigation.md`: "A place is encountered through a query/projection. Placeness is the user's sense of being situated in an intelligible context: what this is about, why things belong here, what can happen here, and how it connects to where they came from." "Coordinates can help during a visit but are not defining properties of a place or entity."

**The Back requirement.** "The user may not know where 'back' is until they see it: 'Oh yes, this is where I was.'" The proposed return contract: recover the experienced context (inquiry, focus, selection, reading position, folds, detail level, representation variants, landmarks; "An equivalent query or the same entity ID alone does not meet the promise"); preserve recognition (visit-local arrangement may be retained; "exact pixels are not a universal requirement"); keep Back distinct "from content Undo, hierarchical Up, dismissing an editor/peek, and inverse zoom"; keep history about the journey ("Coalesce gesture frames; background updates must not fill the trail with machine-generated stops"); separate return from data rollback. Checkpoint "before a potentially disorienting transformation, including a large change of scope, semantic detail or presentation within the same named place."

**Coupling.** "Navigation is the experienced journey through context, rather than a fourth independent display parameter." The history table in `outputs/03-interaction-and-projection.md` section 4 gives the rule per action: glance/inspect create no frame; expanding a relationship family is a reversible current-frame change; follow/explore establishes a checkpoint; open-beside starts an independent frame chain; portal entry has an explicit return boundary.

**What matters for an IDE task-graph panel** (inference): (a) the "Observe a place changing" row: "Distinguish background data/summary updates from the user's own movement; avoid making passive updates feel like unintended travel" and "A substantial passive change must preserve the preceding experienced state as a recovery target", directly applicable when agents mutate the graph while a human is looking at it; (b) Back distinct from editor Undo and from VS Code's own Go Back, with coalesced history; (c) checkpoint before filter/focus changes, and "A selected target filtered out by a new view is not silently rebound to a similar label"; (d) with constant-size nodes: "More full-size appearances compete for space; culling, overlap, aggregation or reprojection alter the overview and need explicit treatment"; (e) "Do not make a rendering library's convenient camera transform the product's zoom contract."

## 4. Renderer architecture

**Contenders** (`outputs/05-renderer-and-precedents.md`): browser-owned HTML/SVG (leading browser candidate); complete toolkit, Flutter leading, with Iced and egui retained; canvas-first browser scene plus DOM editors (Excalidraw precedent; "retain as an alternative with explicit integration cost"); independently composited DOM plus native/GPU scene ("Strongly disfavored... given Daniel's experience" with an earlier knowledge-graph UI's CSS/wgpu clipping regime). Concrete web options named, not chosen: React with React Aria; React Flow (must override default movement/selection; SSR needs explicit node dimensions); Cytoscape/Sigma "for dense analytical graph views after measurement"; Dagre for dependency rank layout. Desktop hosts: Tauri, Electron, or the toolkit's native runner. The selection criterion is "who maintains agreement between painting, clipping, text and input", not a performance ranking.

**Why webviews were excluded, and why this is not the conflict it looks like.** The exact text: "No arbitrary third-party page frames, iframe workaround, browser-extension injection, native webview embedding prototype or streaming prototype. A shell's WebView displaying Manifold's own UI is permitted. A graph portal displays another Manifold projection and is not a website frame." The exclusion targets embedding external web content inside the knowledge UI; hosting the product's own UI in a WebView is explicitly allowed, and Tauri is a listed host candidate. A VS Code webview panel rendering fwdcentaur's own graph is the permitted case (inference; the docs never mention VS Code).

**What was actually measured** (`outputs/verification.md`): no renderer was built or benchmarked. What was executed: structural checks; a Bubble Sets specimen using pinned `bubblesets-js` 3.0.1 checked in Chromium across 36 static combinations, six reveal sequences and six animated reprojections; an association-band illustration; the earlier card preview exercising Follow/Back selection restoration and portal entry; and the three-test Ankurah 0.9.3 model probe. One finding transfers as a test rule: an initial configuration "cut through a member's displayed extent despite center inclusion," so "validate rendered extents rather than assuming the library's center-based membership check guarantees legible containment."

**Implications for a VS Code webview implementation** (inference): the browser-owned HTML/SVG lane applies directly, since the panel is Chromium. Lost or moot: the one-implementation-across-hosts requirement, the Flutter lane, headless capture via the same pipeline. Not covered anywhere in the docs: webview-specific constraints (extension-host message passing, CSP, state loss when a panel is hidden). Still valid: the seven-stage pipeline ending in "Publish geometry, hit regions, accessible reading order and write bindings from the same generation. Reject stale layout results"; the workload accounting table (authorized entities vs appearances vs in-viewport vs mounted DOM vs drawn primitives vs active editors); "Culling is not unmounting"; "Component ownership is not graph membership or adjacency"; "Prefer layers whose generic composition is owned by the chosen browser/toolkit."

## 5. Acceptance and evaluation criteria reusable as tests

From `outputs/03-interaction-and-projection.md` section 9 and `outputs/09-acceptance-and-evaluation.md` (all "proposed evaluations, not tests already run"):

- Arrangement versus assertion (U3): "Fail if proximity/enclosure silently creates domain facts or a query drop guesses a write policy."
- Projection honesty (U6): "Filter and truncate a dense graph; show a compressed path, repeated appearances and unavailable content. Fail if a view implies unsupported adjacency/absence."
- Placeness through reprojection (U4): "Fail on unexplained context/target changes, oscillation, unrecognizable return or accidental content undo; identical coordinates are not required."
- Shared content without surprise (U2): same target in two contexts; edit source, add a local caption, remove one appearance; "Fail if edit/removal scope is misunderstood."
- Evaluation prompts usable verbatim: "Which of these two appearances changes if this title is edited?"; "Is this line stored, derived or a compressed path? What supports it?"; "Did dropping inside this frame change knowledge membership or only placement?"; "What is absent from this view, and what is unknown to this device?"
- Proposed acceptance bar: "no unresolved critical error about edit/deletion/authorization scope in the selected default." Measure "wrong identity/adjacency/effect predictions first, then task completion."
- Stress list: "portal cycles, repeated occurrences, high-degree hubs, two independently zoomed portals, drag across coordinate spaces... a selected item disappearing from a filter."
- Domain test D2 is a task-graph test outright: "Local prerequisite cycle rejection and merged-cycle visualization; general association cycle navigates safely."
- Bubble Set tests: exact `(+n)`, known continuation, unknown completeness, overlapping gates, duplicate appearances, zero visible members, "local reveal without membership mutation", "contour coverage of the displayed target extent, not only its center."
- Performance probes (targets "not requirement"): fixtures of 50/500/5,000 items with a high-degree hub and 10 repeated appearances; typing p95 under 50 ms with 80 visible occurrences; pan/zoom mostly within one 60 Hz frame; command-to-publication p95 under 150 ms; "coordinate preservation is not an acceptance requirement." "Debounce layout, not correctness." "A fast animation followed by a long reflow stall does not pass."
- Layout oracle: compare incremental projection maintenance "under randomized relation add/remove/retarget and missing-endpoint arrival" against "independently specified expected scenes and relation incidences."
- Accessibility gate: "hover/drag are not required for essential operations"; keyboard and single-pointer alternatives to dragging (WCAG cited).

## 6. Prior art most relevant to an IDE task-graph panel

- Juniper (Nobre, Streit, Lex 2018): a queried subgraph as a spanning tree aligned with attribute/adjacency columns, "a concrete alternative to flattening every relationship into lines."
- Ghoniem, Fekete, Castagliola 2005: matrix vs node-link is task-dependent; path finding favors node-link.
- Henry, Bezerianos, Fekete 2008: node duplication helps some tasks, hurts others; clone coloring is insufficient. NodeTrix 2007: hybrid node-link plus matrix for dense local adjacency.
- ELK Layered docs plus Domrös 2023 "Constraining the Model": the five layered phases and the distinction between "ordering preferences, hard ordering and client responsibilities for interactive constraints." Dagre is the only layout library with a cleared license.
- Archambault and Purchase mental-map studies: stability helped location and route tracking, but "challenges universal claims."
- Bubble Sets (Collins, Penn, Carpendale 2009) plus `bubblesets-js` (the only technique actually exercised in a browser check).
- Edge-Path Bundling (Wallinger 2021/22): why bundling creates false junctions.
- Pad++, Furnas fisheye, and Bederson 2011 on the limits of universal zoomable interfaces.
- TheBrain: inferred risks "automatic focus pivots can disrupt a train of thought; a selected local neighborhood can look like the entire graph."
- Obsidian: local graph depth/filters; Canvas's rule that text cards are not backlinks until converted; JSON Canvas 1.0 as a lossy export target.
- tldraw issue #8307 and Excalidraw's low-resolution bitmap caches during zoom, as dense-scene precedents.
- Shipman and Marshall "Formality Considered Harmful": let tentative organization precede formalization.
- For line annotations specifically: the W3C Web Annotation Data Model (selectors, ambiguity "must be reported, not silently resolved to the first match"), Hypothesis fuzzy anchoring, and Yjs relative positions, with the rule "Every target states whether it follows live edits, preserves a historical version, or preserves a particular quotation" and "Offsets must declare units and normalization."

**Net conflicts** (summarizer's view): the webview exclusion is not one. Real mismatches are the one-implementation and daemon premises, the docs' silence on webview-host constraints, and the scale of Manifold's ambitions (interior pointing, presence, sharing) that a task panel does not need. Everything in sections 2, 3 and 5 is host-independent and transfers as written, with the standing caveat the docs repeat throughout: these are design hypotheses and proposed tests, "not measured user results."
